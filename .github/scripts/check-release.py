#!/usr/bin/env python3
"""发布包 tag／版本／SHA256 校验（build-release.yml 的发布门禁；ADR-039／T183／ADR-043／ADR-083）。

为什么单独一层：构建、打包、上传、发布是不同进程，任何一步都可能让
“文件名里的版本”“tag 版本”“实际字节”三者脱钩。Release 一旦公开就会被镜像与缓存，
故上传前用一次独立复算把三者对齐：

  1. 目录内发布包在过滤范围内必须唯一——多个说明打包污染，缺失说明打包未执行；
     且必须非空且是有效归档（Windows 为 zip，FreeBSD 为 tar.gz，0 字节或损坏一律
     失败）。同一目录允许并存 windows-amd64／windows-arm64／freebsd-amd64 三个包
     （ADR-043／ADR-083），不带 `--arch` 时逐个全部校验；
  2. 文件名中的 `<version>` 与 tag（去掉 `v` 前缀）一致，或 tag 形如
     `<version>-<预发布标识>`（如 `v0.1.0-rc.1`）；
  3. `.sha256` 记录的哈希与重新计算的包哈希一致；记录必须恰为 sha256sum 的两个
     字段（64 位十六进制与文件名，文件名允许前缀 `*`）且文件名等于当前包；
  4. 传入 `--record` 时（须同时指定 `--arch`），与打包脚本写出的安装验证记录首部行
     `package = <包>（<字节> 字节，sha256=<64hex>）` 交叉校验包名、字节数与 sha256
     ——任一不符即失败，发布作业据此中止。

任一条不满足即退出 1，并打印 `tag = ...`／`package_version = ...` 等可归因字段；
不以 0 或空值冒充通过（docs/guides/signal-chain-completeness.md）。

只用 Python 标准库；写出文件为 UTF-8 无 BOM、LF，内容为 sha256sum 兼容格式。

用法：
    python .github/scripts/check-release.py --dir target/dist --write
    python .github/scripts/check-release.py --dir dist --tag v0.1.0
    python .github/scripts/check-release.py --dir dist --tag v0.1.0 --arch amd64 \
        --record artifacts/install-verify-windows-amd64.txt
    python .github/scripts/check-release.py --dir dist --tag v0.1.0 --arch freebsd-amd64 \
        --record artifacts/install-verify-freebsd-amd64.txt
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import sys
import tarfile
import zipfile

#: 发布包命名与形态（ADR-043 Windows zip；ADR-083 FreeBSD tar.gz）。
WINDOWS_ZIP_PATTERN = re.compile(r"^whcode-(?P<version>.+)-windows-(?P<arch>amd64|arm64)\.zip$")
FREEBSD_TAR_PATTERN = re.compile(r"^whcode-(?P<version>.+)-freebsd-(?P<arch>amd64)\.tar\.gz$")
HEX_PATTERN = re.compile(r"^[0-9a-f]{64}$")
#: 安装验证记录首部行：package-windows.py／package-freebsd.sh 的 write_record() 决定此格式。
RECORD_PATTERN = re.compile(
    r"^package\s*=\s*(?P<name>[^（）\s]+)\s*（(?P<bytes>\d+)\s*字节，sha256=(?P<digest>[0-9a-fA-F]{64})）$"
)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def match_package(name: str) -> re.Match[str] | None:
    """按两种形态匹配包名；返回匹配结果（group("platform") 为平台键）。"""
    match = WINDOWS_ZIP_PATTERN.match(name)
    if match is not None:
        return match
    return FREEBSD_TAR_PATTERN.match(name)


def platform_key(match: re.Match[str]) -> str:
    """包的平台键：windows-amd64／windows-arm64／freebsd-amd64。"""
    if match.re is WINDOWS_ZIP_PATTERN:
        return f"windows-{match.group('arch')}"
    return f"freebsd-{match.group('arch')}"


def find_packages(directory: pathlib.Path, arch: str | None) -> tuple[list[pathlib.Path], str | None]:
    """在目录中定位发布包；返回 (包列表, 问题描述)。

    `arch` 取 windows 架构（amd64／arm64）时只筛 Windows zip；取 freebsd-amd64 时只筛
    FreeBSD tar.gz；None 时取全部平台。
    """
    if arch in ("amd64", "arm64"):
        scope = f"windows-{arch}"
    elif arch == "freebsd-amd64":
        scope = arch
    else:
        scope = "windows-<amd64|arm64>／freebsd-amd64"
    if not directory.is_dir():
        return [], f"{directory} 不存在（打包产物目录缺失）"
    candidates: list[pathlib.Path] = []
    for path in sorted(directory.glob("whcode-*")):
        if not path.is_file():
            continue
        match = match_package(path.name)
        if match is None:
            continue
        if arch in ("amd64", "arm64") and platform_key(match) != f"windows-{arch}":
            continue
        if arch == "freebsd-amd64" and platform_key(match) != arch:
            continue
        candidates.append(path)
    if not candidates:
        return [], f"{directory} 下未找到 whcode-*-{scope} 发布包（打包是否已执行？）"
    if arch is None:
        keys = [platform_key(match_package(path.name)) for path in candidates]
        duplicates = sorted({key for key in keys if keys.count(key) > 1})
        if duplicates:
            names = "、".join(path.name for path in candidates)
            return [], f"{directory} 下同一平台存在多个发布包，必须唯一：{names}（重复平台：{'、'.join(duplicates)}）"
    return candidates, None


def checksum_path(package_path: pathlib.Path) -> pathlib.Path:
    return package_path.with_name(package_path.name + ".sha256")


def validate_archive(package_path: pathlib.Path) -> list[str]:
    """发布包必须非空且是有效归档：0 字节与损坏文件都不得冒充发布产物。"""
    if package_path.stat().st_size == 0:
        return [f"{package_path.name}：发布包为空（0 字节），拒绝作为发布产物"]
    if package_path.name.endswith(".zip"):
        if not zipfile.is_zipfile(package_path):
            return [f"{package_path.name}：发布包 zip 不是有效归档（zipfile.is_zipfile 为假）"]
    else:
        if not tarfile.is_tarfile(package_path):
            return [f"{package_path.name}：发布包不是有效 tar 归档（tarfile.is_tarfile 为假）"]
    return []


def record_problems(record_path: pathlib.Path, package_path: pathlib.Path, digest: str) -> list[str]:
    """把安装验证记录首部行与当前包逐项对齐；记录缺失或无法解析也算失败。"""
    if not record_path.is_file():
        return [f"缺少安装验证记录 {record_path.name}（--record 指定的文件不存在）"]
    try:
        text = record_path.read_text(encoding="utf-8")
    except OSError as error:
        return [f"无法读取安装验证记录 {record_path.name}：{error}"]

    line = next((item.strip() for item in text.splitlines() if item.strip().startswith("package =")), None)
    if line is None:
        return [f"{record_path.name} 缺少 package = … 首部行，无法交叉校验包名、字节数与 sha256"]
    match = RECORD_PATTERN.match(line)
    if match is None:
        return [f"{record_path.name} 的 package 首部行无法解析：{line}"]

    problems: list[str] = []
    recorded_name = match.group("name")
    if recorded_name != package_path.name:
        problems.append(f"安装验证记录包名 {recorded_name} 与当前包 {package_path.name} 不一致")
    recorded_bytes = int(match.group("bytes"))
    actual_bytes = package_path.stat().st_size
    if recorded_bytes != actual_bytes:
        problems.append(f"安装验证记录字节数 {recorded_bytes} 与实测 {actual_bytes} 不一致")
    recorded_digest = match.group("digest").lower()
    if recorded_digest != digest:
        problems.append(f"安装验证记录 sha256 {recorded_digest} 与实测 {digest} 不一致")
    return problems


def write_mode(directory: pathlib.Path) -> int:
    packages, problem = find_packages(directory, None)
    if problem:
        print(f"FAIL: {problem}")
        return 1
    problems: list[str] = []
    for package_path in packages:
        problems.extend(validate_archive(package_path))
    if problems:
        for item in problems:
            print(f"FAIL: {item}")
        return 1
    for package_path in packages:
        digest = sha256(package_path)
        target = checksum_path(package_path)
        target.write_text(f"{digest}  {package_path.name}\n", encoding="utf-8", newline="\n")
        print(f"package = {package_path.name}")
        print(f"sha256 = {digest}")
        print(f"已写入：{target}")
    return 0


def tag_mode(directory: pathlib.Path, tag: str, arch: str | None, record: pathlib.Path | None) -> int:
    packages, problem = find_packages(directory, arch)
    if problem:
        print(f"FAIL: {problem}")
        print(f"tag = {tag or '未取得'}")
        return 1
    if record is not None and arch is None:
        print("FAIL: --record 需与 --arch 同用（安装验证记录按平台一一对应，ADR-043／ADR-083）")
        return 1

    all_problems: list[str] = []
    summaries: list[str] = []
    for package_path in packages:
        match = match_package(package_path.name)
        version = match.group("version") if match else ""
        package_problems = [f"{package_path.name}：{item}" for item in validate_archive(package_path)]
        digest = ""
        if not package_problems:
            digest = sha256(package_path)

            # tag 的前缀 `v` 只用于人类可读；版本比对一律用去前缀后的字符串。
            tag_version = tag[1:] if tag.startswith("v") else tag
            if version == "":
                package_problems.append(f"{package_path.name}：无法从文件名解析版本号")
            elif not (tag_version == version or tag_version.startswith(f"{version}-")):
                package_problems.append(
                    f"{package_path.name}：tag 版本 {tag_version} 与包版本 {version} 不一致（预发布须为 {version}-…）"
                )

            recorded_file = checksum_path(package_path)
            if not recorded_file.is_file():
                package_problems.append(f"{package_path.name}：缺少 {recorded_file.name}（先用 --write 生成）")
            else:
                parts = recorded_file.read_text(encoding="utf-8").split()
                if len(parts) != 2:
                    package_problems.append(
                        f"{recorded_file.name}：不是 sha256sum 格式（应恰为 64 位十六进制与文件名两个字段，"
                        f"实际 {len(parts)} 个）"
                    )
                else:
                    recorded, recorded_name = parts
                    if not HEX_PATTERN.match(recorded):
                        package_problems.append(f"{recorded_file.name}：首字段非 64 位十六进制")
                    elif recorded != digest:
                        package_problems.append(f"{recorded_file.name}：SHA256 不一致（记录 {recorded}，实测 {digest}）")
                    if recorded_name.lstrip("*") != package_path.name:
                        package_problems.append(f"{recorded_file.name}：记录的文件名 {recorded_name} 不是 {package_path.name}")

            if record is not None:
                package_problems.extend(
                    f"{package_path.name}：{item}" for item in record_problems(record, package_path, digest)
                )

        all_problems.extend(package_problems)
        summaries.append(f"package = {package_path.name}（{version or '未取得'}）")
        summaries.append(f"sha256 = {digest or '未取得'}")

    for item in all_problems:
        print(f"FAIL: {item}")
    print(f"tag = {tag or '未取得'}")
    for line in summaries:
        print(line)
    if all_problems:
        return 1
    suffix = f"，安装验证记录 {record.name} 与包一致" if record is not None else ""
    print(f"发布校验通过：tag={tag} 与各包版本一致，SHA256 与记录一致{suffix}")
    return 0


def main() -> int:
    # Windows 控制台代码页非 UTF-8 时中文输出会抛 UnicodeEncodeError，与其余门禁脚本同处理。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description="发布包 tag／版本／SHA256 校验")
    parser.add_argument(
        "--dir", required=True, help="发布包所在目录（如 target/dist／dist）"
    )
    parser.add_argument(
        "--arch",
        default=None,
        choices=["amd64", "arm64", "freebsd-amd64"],
        help="只校验指定平台（amd64／arm64 指 Windows zip；freebsd-amd64 指 FreeBSD tar.gz，ADR-083；缺省逐个校验全部）",
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true", help="写入 <包>.sha256")
    mode.add_argument("--tag", default=None, help="校验 tag 与包版本、SHA256 一致")
    parser.add_argument(
        "--record",
        default=None,
        metavar="PATH",
        help="安装验证记录（与 --tag --arch 同用：交叉校验包名、字节数与 sha256；--write 模式不需要）",
    )
    args = parser.parse_args()

    directory = pathlib.Path(args.dir)
    if args.write:
        return write_mode(directory)
    return tag_mode(directory, args.tag, args.arch, pathlib.Path(args.record) if args.record else None)


if __name__ == "__main__":
    sys.exit(main())
